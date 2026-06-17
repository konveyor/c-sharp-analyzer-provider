using System.Collections.Generic;
using System.ServiceModel;
using LegacyApp.Models;

namespace LegacyApp.Services
{
    [ServiceContract(Namespace = "http://contoso.com/services/2010")]
    public interface ICustomerService
    {
        [OperationContract]
        Customer GetCustomer(int id);

        [OperationContract]
        List<Customer> SearchCustomers(string name);

        [OperationContract]
        void CreateCustomer(Customer customer);

        [OperationContract]
        List<Order> GetCustomerOrders(int customerId);

        [OperationContract]
        void CancelOrder(int orderId);
    }
}
